/*
  Copyright 2021 Grégory Soutadé

  This file is part of uPDFParser.

  uPDFParser is free software: you can redistribute it and/or modify
  it under the terms of the GNU Lesser General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  uPDFParser is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU Lesser General Public License for more details.

  You should have received a copy of the GNU Lesser General Public License
  along with uPDFParser. If not, see <http://www.gnu.org/licenses/>.
*/

#include <unistd.h>
#include <algorithm>

#include "uPDFTypes.h"
#include "uPDFParser_common.h"

namespace uPDFParser
{
    Name::Name(const std::string& name):
	DataType(DataType::TYPE::NAME)
    {
	_value = name;
    }

    String::String(const std::string& value):
	DataType(DataType::TYPE::STRING)
    {
	_value = value;
    }

    HexaString::HexaString(const std::string& value):
	DataType(DataType::TYPE::HEXASTRING)
    {
	_value = value;
    }

    std::string Integer::str()
    {
	std::string sign("");
	// Sign automatically added for negative numbers
	if (_signed && _value >= 0)
	    sign = "+";

	return " " + sign + std::to_string(_value);
    }
    
    std::string Real::str()
    {
	std::string res;
	std::string sign("");
	if (_signed && _value >= 0)
	    sign = "+";

	res = " " + sign + std::to_string(_value);
	std::replace( res.begin(), res.end(), ',', '.');

	return res;
    }

    std::string Array::str()
    {
	std::string res("[");
	std::vector<DataType*>::iterator it;

	for(it = _value.begin(); it!=_value.end(); it++)
	{
	    /* These types has already a space in front */
	    if ((*it)->type() != DataType::TYPE::INTEGER &&
		(*it)->type() != DataType::TYPE::REAL &&
		(*it)->type() != DataType::TYPE::REFERENCE)
	    {
		if (res.size() > 1)
		    res += " ";
		res += (*it)->str();
	    }
	    else
	    {
		if (res.size() > 1)
		    res += (*it)->str();
		/* First time, remove front space*/
		else
		    res += (*it)->str().substr(1);
	    }
	}

	if (res.size() == 1)
	    res += " ";
	
	return res + "]";
    }

    void Dictionary::addData(const std::string& key, DataType* value)
    {
	_value[key] = value;
    }
    
    std::string Dictionary::str()
    {
	std::string res("<<");
	std::map<std::string, DataType*>::iterator it;
	
	for(it = _value.begin(); it!=_value.end(); it++)
	{
	    res += std::string("/") + it->first;
	    if (it->second)
		res += it->second->str();
	}
	    
	return res + std::string(">>\n"); 
   }

    std::string Stream::str()
    {
	std::string res = "stream\n";
	const char* streamData = (const char*)data(); // Force reading if not in memory
	res += std::string(streamData, _dataLength);
	res += "\nendstream\n";

	return res;
    }
    
    unsigned char* Stream::data()
    {
	if (!_data)
	{
	    if (!fd)
		EXCEPTION(INVALID_STREAM, "Accessing data, but no file descriptor supplied");

	    _dataLength = endOffset - startOffset;
	    _data = new unsigned char[_dataLength];
	    freeData = true;

	    lseek(fd, startOffset, SEEK_SET);
	    int ret = ::read(fd, _data, _dataLength);

	    if ((unsigned int)ret != _dataLength)
		EXCEPTION(INVALID_STREAM, "Not enough data to read (" << ret << ")");
	}
	
	return _data;
    }

    void Stream::setData(unsigned char* data, unsigned int dataLength, bool freeData)
    {
	if (_data && freeData)
	    delete[] _data;

	dict.deleteKey("Length");
	dict.addData("Length", new Integer(dataLength));

	this->_data = data;
	this->_dataLength = dataLength;
	this->freeData = freeData;
    }
}
